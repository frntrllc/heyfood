# heyfood

Native command-line access to personalized food and dietary guidance from
[hello.food](https://hello.food).

This repository is moving the CLI and interactive terminal experience to Rust.
The recovery release target is `0.5.0`. Publication is suspended while the
installed-artifact journeys, native distribution, and exact-SHA release gates
are completed. The immutable `v0.4.0` and `v0.4.1` releases are unsupported and
must not be installed.

## Installation

The hosted native installer intentionally fails closed until the reviewed
`v0.5.0` artifact is published and passes its public smoke tests. There is no
supported binary installation command during qualification.

To inspect immutable installer bytes before running them, replace `REVISION`
with a reviewed full commit SHA and verify the separately reviewed checksum:

```bash
REVISION="<full-reviewed-commit-sha>"
curl -fsSLO "https://raw.githubusercontent.com/frntrllc/heyfood/${REVISION}/install.sh"
curl -fsSLO "https://raw.githubusercontent.com/frntrllc/heyfood/${REVISION}/install.sh.sha256"
if command -v sha256sum >/dev/null 2>&1; then
  sha256sum -c install.sh.sha256
else
  shasum -a 256 -c install.sh.sha256
fi
less install.sh
bash install.sh
```

## Build from source

The native workspace requires the Rust toolchain declared in `Cargo.toml`.

```bash
git clone https://github.com/frntrllc/heyfood.git
cd heyfood
cargo build --release --locked --package heyfood-bin
./target/release/heyfood --version
./target/release/heyfood --help
```

After qualification, GitHub Releases and the hosted installer will be the
supported public binary distribution path. Building a reviewed source revision
remains supported for contributors.

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

Native `0.4.0` credentials predate the Grocery and Health scopes. If a command
reports `authorization_scope_upgrade_required`, approve the explicit upgrade:

```bash
heyfood login
```

OAuth refresh cannot add authority. Login preserves the existing credentials
through device approval and session exchange, verifies the complete expanded
grant, and only then replaces both native credential stores.

## Active commands

```bash
heyfood ask "What can I eat?"
heyfood reply --conversation-id CONVERSATION_ID "The second option"
heyfood log "I ate the first option"
heyfood item "pad thai at Pismo's"
heyfood grocery list
heyfood health status
heyfood health show
```

`reply` requires an explicit `--conversation-id` in this cut because native
conversation persistence is not active. `ask`, `log`, and `item` may also use
`--conversation-id` to continue a known conversation. All four commands accept
an optional coordinate pair:

```bash
heyfood ask --latitude 35.28 --longitude -120.66 "What can I order nearby?"
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

After registration, run `heyfood` without a subcommand to enter the native Rust
TUI. The composer remains editable while responses stream, keeps bounded
process-local prompt history, and preserves conversation continuity only for
the lifetime of the process.

Interactive controls include Enter to send, Shift+Enter or Ctrl+J for a
newline, Up/Down for prompt history, PageUp/PageDown for scrollback, Ctrl+C to
stop an active turn, and Ctrl+D or `/exit` to leave. Use `/help` for the current
command registry, `/new` for a fresh conversation, `/clear` to clear visible
scrollback, and `/status` to inspect session readiness.

Profile, household, location, and voice panels remain in active Rust
implementation. Their existing one-shot compatibility routes continue to fail
closed where the native workflow is not complete.

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
