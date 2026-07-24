# heyfood

> [!CAUTION]
> **Native installation is suspended. Do not install or use v0.4.0 or v0.4.1.**
> Both releases were published before release authorization and are unsupported.
> The hosted installer intentionally exits without installing anything until a
> supported release is available.

Native command-line access to personalized food and dietary guidance from
[hello.food](https://hello.food).

This repository is moving the CLI and interactive terminal experience to Rust.
The recovery release target is `0.5.0`. Publication is suspended while the
installed-artifact journeys, native distribution, and exact-SHA release gates
are completed. The immutable `v0.4.0` and `v0.4.1` releases are unsupported and
must not be installed.

See the [current capability and distribution status](docs/CAPABILITY_STATUS.md)
before evaluating the client.

## Installation status

There is no supported public native binary today. The hosted command currently
prints the release status without installing anything:

```bash
curl -fsSL https://hey.food/install.sh | bash
```

It prints the release-incident notice to stderr and exits `1`. Do not pin
v0.4.0 or v0.4.1 to bypass the suspension. A successful install command will
return here only after a replacement release passes testing and receives
explicit release approval.

## Inspect or build from source

The native workspace requires the Rust toolchain declared in `Cargo.toml`.

```bash
git clone https://github.com/frntrllc/heyfood.git
cd heyfood
cargo build --release --locked --package heyfood-bin
./target/release/heyfood --version
./target/release/heyfood --help
```

GitHub Releases and the hosted installer are the only supported public binary
distribution path. Building a reviewed source revision is available for
contributors, but it is not a supported substitute for the suspended release.

## Register

Connect a hello.food account before running an agent command:

```bash
heyfood register
```

Registration starts the native device-authorization flow and prints a URL and
short approval code. Identity verification and current Terms and Privacy
acceptance happen on `auth.hello.food`; the hosted page offers the SMS and email
methods enabled for the deployment. SMS registration is US-only.

On a headless machine, keep browser launch disabled:

```bash
heyfood register --device --no-browser
```

For automation, `--json` also prevents browser launch and emits one terminal
JSON result after approval, expiry, cancellation, or failure:

```bash
heyfood register --device --no-browser --json --timeout 600
```

The native client persists credentials only after authorization, session
exchange, and response-contract validation all succeed. If it reports an
uncertain session-exchange or persistence outcome, do not start another
registration attempt until account state is reconciled.

Older native credentials may predate the Grocery and Health scopes. If a command
reports `authorization_scope_upgrade_required`, approve the explicit upgrade:

```bash
heyfood login
```

OAuth refresh cannot add authority. Login preserves the existing credentials
through device approval and session exchange, verifies the complete expanded
grant, and only then replaces both native credential stores.

## Current Rust command surface

```bash
heyfood ask "What can I eat?"
heyfood reply --conversation-id CONVERSATION_ID "The second option"
heyfood log "I ate the first option"
heyfood item "pad thai at Pismo's"
heyfood grocery show
heyfood grocery exclusions
heyfood grocery never --list-id UUID --version 4 "raw onion"
heyfood health status
heyfood health show
```

`reply` requires an explicit `--conversation-id` in this cut because native
conversation persistence is not active. `ask`, `log`, and `item` may also use
`--conversation-id` to continue a known conversation. All four commands accept
an optional coordinate pair:

```bash
heyfood ask --lat 35.28 --lng -120.66 "What can I order nearby?"
```

If command text is omitted and stdin is not a terminal, the client reads the
UTF-8 prompt from stdin:

```bash
printf '%s\n' "What can I eat?" | heyfood ask --json
```

The product uses “generally safer,” “risky,” “avoid,” and “unable to evaluate”
rather than presenting food as absolutely safe.

## Machine output

Place global flags before or after the subcommand. `--json` emits exactly one
ANSI-free JSON value on stdout; progress and human diagnostics use stderr.
`--raw` is a deprecated alias for `--json`.

```bash
heyfood --json ask "Can I eat pad thai?"
heyfood item "pad thai" --json
```

Failures use a stable error envelope and a nonzero exit status. Errors with an
uncertain server-side outcome include `error.outcome_uncertain: true` so callers
do not retry a potentially committed operation blindly. See the
[CLI process contract](docs/CLI_CONTRACT.md).

## Interactive terminal

On this draft branch, registration continues into the native Rust TUI and an
authenticated bare `heyfood` launches it directly. This is source-level preview
work, not a supported release; the hosted installer remains suspended. The
composer remains editable while responses stream, keeps bounded process-local
prompt history, and preserves conversation continuity only for the lifetime of
the process.

Interactive controls include Enter to send, Shift+Enter or Ctrl+J for a
newline, Up/Down for prompt history, PageUp/PageDown for scrollback, Ctrl+C to
stop an active turn, and Ctrl+D or `/exit` to leave. In native-audio builds,
Ctrl+Space, F8, or `/voice` starts/stops memory-only capture and places the
validated transcript in the composer for editing before submission. Use
`/help` for the current command registry, `/new` for a fresh conversation,
`/clear` to clear visible scrollback, and `/status` to inspect session
readiness.

Grocery, Health, profile, household, location, and status panels are connected
on the draft branch. Grocery list cards expose stable IDs, provenance, member
screening, substitutions, and never-buy exclusions. Conversational item-list
proposals support typed accept/cancel decisions in the TUI. `grocery export
LIST_ID --out FILE` writes annotations through an owner-only, exclusive,
symlink-safe file path; `--overwrite` opts into atomic replacement. Proposal
editing and the native voice vertical are present in source. Menu Watch,
installed-artifact showcase execution, and real-hardware voice qualification
remain incomplete release gates. Hidden compatibility routes continue to fail
closed where a native workflow is not complete.

## Development

Run the native checks from the repository root:

```bash
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo xtask verify-stable-contracts
cargo xtask verify-grocery-contracts
cargo xtask verify-assets
```

Hash-pinned JSON under `fixtures/contracts/` and `schemas/` is checked out with
LF line endings on every platform through `.gitattributes`; do not rewrite
approved contract bytes or update their hashes as part of unrelated changes.

Additional project references:

- [Capability and distribution status](docs/CAPABILITY_STATUS.md)
- [Development setup](DEVELOPMENT.md)
- [Contributing](CONTRIBUTING.md)
- [Changelog](CHANGELOG.md)
- [Release process](RELEASING.md)
- [Security policy](SECURITY.md)
- [Code of Conduct](CODE_OF_CONDUCT.md)

## Uninstall

The installer prints the exact installed path and removal command. For the
default directory:

```bash
rm "$HOME/.local/bin/heyfood"
```

This removes only the native executable. The current native cut does not expose
logout or account-state removal yet, so uninstalling does not revoke the hosted
authorization or delete owner-only local account state.

## License and project boundary

Copyright 2026 FRNTR, LLC.

The heyfood CLI distribution is licensed under the
[Apache License 2.0](LICENSE). The license applies to this client and explicitly
published public assets. It does not license the proprietary hello.food
backend, hosted service, intelligence, models, prompts, data, evaluation rules,
or infrastructure.
