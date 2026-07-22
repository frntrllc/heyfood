# heyfood command grammar

This document describes the current Rust command surface. It does not document
legacy Python behavior or hidden compatibility topology.

## Active top-level commands

```text
register      create and connect a hello.food account
login         authenticate again or approve expanded scopes
ask           ask the hosted agent a one-shot question
reply         continue an explicit conversation id
log           log a meal through the hosted agent
item          assess a food or menu item
grocery       read, prepare, export, and confirm Grocery operations
health        read health context and manage the Oura integration
completion    print shell completion syntax
```

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
heyfood ask --latitude 35.28 --longitude -120.66 "What can I order nearby?"
```

## Registration and login

```bash
heyfood register
heyfood register --device --no-browser
heyfood register --device --no-browser --json --timeout 600
heyfood login
```

`--json` suppresses browser launch and interactive prompts. Device authorization
still requires one human approval on `auth.hello.food`. Refresh never widens
scopes; `login` is the explicit scope-upgrade path.

## Grocery

```bash
heyfood grocery list
heyfood grocery add --list-id UUID --version VERSION "red lentils" "onion"
heyfood grocery remove --list-id UUID --version VERSION ITEM_OR_INDEX
heyfood grocery state --list-id UUID --version VERSION ITEM purchased
heyfood grocery export UUID --format markdown
heyfood grocery confirm --decision accept --proposal-stdin < proposal.json
```

Mutation commands prepare a proposal and do not commit it. Confirmation reads
the proposal from stdin so authorization material does not enter shell history
or process arguments.

## Health

```bash
heyfood health status
heyfood health show
heyfood health connect oura
heyfood health sync oura
heyfood health disconnect oura --yes
```

Oura is the current direct CLI provider. Apple Health summaries are acquired by
the hello.food app and exposed only as provider-labeled hosted context; the CLI
does not access HealthKit.

## Global process controls

```text
--json       one ANSI-free JSON value on stdout
--no-color   disable ANSI styling
--no-banner  disable decorative branding
--verbose    privacy-safe diagnostics on stderr
--no-input   never prompt for missing local input
```

`--raw` is a deprecated alias for `--json`.

## Unavailable compatibility topology

Interactive chat/TUI, onboarding, profile management, restaurant search,
saved location, recommendation, menu, recipe, household management, voice,
configuration, diagnostics, logout, and account management are not active Rust
commands. Some names remain hidden for migration topology only and return
`command_not_available`.

Bare `heyfood` prints informational next steps and exits. It does not launch a
TUI, browser, registration flow, onboarding, or network request.
