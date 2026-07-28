# heyfood

> [!CAUTION]
> **Do not install or use v0.4.0 or v0.4.1.**
> Both releases were published before release authorization and remain
> unsupported. The supported replacement is `v0.5.0`.

Native command-line access to personalized food and dietary guidance from
[hello.food](https://hello.food).

The CLI and interactive terminal experience are implemented in Rust. The
supported recovery release is `0.5.0` for macOS and Linux. The immutable
`v0.4.0` and `v0.4.1` releases remain unsupported and must not be installed.

See the [current capability and distribution status](docs/CAPABILITY_STATUS.md)
before evaluating the client.

## Install

Install the supported native `v0.5.0` binary on macOS or Linux:

```bash
curl -fsSL https://hey.food/install.sh | bash
```

The installer downloads the archive for the current CPU, verifies its checksum
and exact version, and atomically installs it under the current user without
`sudo` or shell-profile edits. Windows distribution is deferred to `v0.5.1`.

## Inspect or build from source

The native workspace requires the Rust toolchain declared in `Cargo.toml`.

```bash
git clone https://github.com/frntrllc/heyfood.git
cd heyfood
cargo build --release --locked --package heyfood-bin
./target/release/heyfood --version
./target/release/heyfood --help
```

GitHub Releases and the hosted installer are the supported public binary
distribution paths. Building a reviewed source revision remains available for
contributors.

## Agent discovery

The v0.6.0 source candidate can explain its exact installed automation
contract without repository access, credentials, or a network connection:

```bash
heyfood agent describe
heyfood agent guide --format markdown
heyfood agent schema --list
heyfood agent doctor
```

Agents must use this self-description rather than scrape the interactive TUI
or infer authority from `--help`. Codex/Claude setup and the local read-only
MCP server are separately qualified increments and are not implied by the
Phase 1 command family.

## Connect an account

The supported public `v0.5.0` artifact predates the account-neutral first-run
flow: bare launch begins explicit account creation, and fresh-machine login for
an existing account is not yet supported. This is the known successor-artifact
gap tracked in [#29](https://github.com/frntrllc/heyfood/issues/29).

Current source gives a fresh user one browser-based choice to sign in or create
a hello.food account, then continues directly into onboarding and the TUI:

```bash
heyfood
```

Use `heyfood login` to connect an existing account directly. On an already
connected machine, the same command stages and atomically replaces the current
authorization with the canonical supported scope set. Use `heyfood register`
when account creation is explicitly intended.

Account connection starts the native device-authorization flow and prints a
URL and short approval code. Identity verification and current Terms and
Privacy acceptance happen on `auth.hello.food`; the hosted page offers the SMS
and email methods enabled for the deployment. SMS registration is US-only.

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

Older native credentials may predate the Grocery or Menu Watch scopes. If a
command reports `authorization_scope_upgrade_required`, approve the explicit
authorization replacement:

```bash
heyfood login
```

OAuth refresh cannot change authority. Login preserves the existing credentials
through device approval and session exchange, verifies the complete canonical
supported grant, and only then replaces both native credential stores. The
replacement may add Grocery or Menu Watch authority and removes scopes for
deferred capabilities such as Health.

## Current Rust command surface

```bash
heyfood ask "What can I eat?"
heyfood reply --conversation-id CONVERSATION_ID "The second option"
heyfood log "I ate the first option"
heyfood item "pad thai at Pismo's"
heyfood grocery show
heyfood grocery exclusions
heyfood grocery never --list-id UUID --version 4 "raw onion"
heyfood watch list
heyfood watch add RESTAURANT_UUID --weekday thursday --hour 9 --notify
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

Account connection continues into the native Rust TUI and an authenticated bare
`heyfood` launches it directly. The composer remains editable while responses
stream, keeps bounded process-local prompt history, and preserves conversation
continuity only for the lifetime of the process.

The bounded `v0.5.0` recovery release produces exactly four native archives:
macOS Apple Silicon, macOS Intel, Linux ARM64, and Linux x64.
Windows distribution is deferred to `v0.5.1`; ordinary Windows compile, test,
Clippy, credential, and packaging qualification remains active in CI. Health,
item-level Menu Watch diff detail, native voice, full legacy parity, and the
complete twelve-stage showcase are future work rather than `v0.5.0` release
gates.

Interactive controls include Enter to send, Shift+Enter or Ctrl+J for a
newline, Up/Down for prompt history, PageUp/PageDown for scrollback, Ctrl+C to
stop an active turn, and Ctrl+D or `/exit` to leave. In native-audio builds,
Ctrl+Space, F8, or `/voice` starts/stops memory-only capture and places the
validated transcript in the composer for editing before submission. Use
`/help` for the current command registry, `/new` for a fresh conversation,
`/clear` to clear visible scrollback, and `/status` to inspect session
readiness.

Grocery, Menu Watch, profile, household, location, and status panels are
included in `v0.5.0`. Health integrations are explicitly deferred from
the supported `v0.5.0` contract: `health` is absent from root help and shell
completion, `/health` is absent from TUI discovery, and fresh grants do not
request Health or integration-management scopes. Retained internal
provider-neutral contracts are future work, not a release capability.

Grocery list cards expose stable IDs, provenance, member
screening, substitutions, and never-buy exclusions. Conversational item-list
proposals support typed accept/cancel decisions in the TUI. `grocery export
LIST_ID --out FILE` writes annotations through an owner-only, exclusive,
symlink-safe file path; `--overwrite` opts into atomic replacement. Proposal
editing and the native voice vertical are present in source. `heyfood watch`
and `/watch` create/list/remove or display subscriptions using the deployed
`menu:watch` contract. The TUI renders the latest account-owned change summary
with source, freshness, and provenance; item-level added, removed, modified,
and price-change detail remains follow-on work. The broader installed-artifact
showcase and real-hardware voice qualification remain post-`v0.5.0`
conformance work. Hidden compatibility routes continue to fail closed where a
native workflow is not complete.

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
