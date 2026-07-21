# heyfood

Native command-line access to personalized food and dietary guidance from
[hello.food](https://hello.food).

This repository is moving the CLI to Rust. The current public native cut is
deliberately small: `register`, `ask`, `reply`, `log`, and `item` are the only
active product commands. Guidance comes from the hosted hello.food service and
is not a substitute for professional medical advice or emergency care.

## Installation

On macOS or glibc-based Linux, install the native Rust release with:

```bash
curl -fsSL https://hey.food/install.sh | bash
```

The [installer source](install.sh) selects the matching Apple Darwin or GNU
Linux archive for the current `aarch64` or `x86_64` CPU. It downloads that
archive and `SHA256SUMS` from the same GitHub Release, verifies the checksum and
single-file archive policy, runs the downloaded executable's exact `--version`
check, and atomically installs it to
`${HEYFOOD_BIN_DIR:-$HOME/.local/bin}/heyfood`. It never uses `sudo`, edits shell
startup files, or requires Python or `pipx`.

Pin an exact release or choose another user-owned installation directory with:

```bash
curl -fsSL https://hey.food/install.sh | HEYFOOD_VERSION=0.4.0 bash
curl -fsSL https://hey.food/install.sh | \
  HEYFOOD_BIN_DIR="$HOME/bin" bash
```

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

GitHub Releases and the hosted installer are the only supported public binary
distribution path. Building a reviewed source revision remains supported for
contributors.

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

## Active commands

```bash
heyfood ask "What can I eat?"
heyfood reply --conversation-id CONVERSATION_ID "The second option"
heyfood log "I ate the first option"
heyfood item "pad thai at Pismo's"
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

## What is not active yet

The native Rust binary does not currently provide the legacy Python CLI's
interactive chat/TUI, onboarding, profile, account-management, restaurant
search, saved location, recommendation, menu, recipe, household, voice,
Grocery, Health, context, configuration, or diagnostic workflows. Hidden
compatibility topology may recognize some old command names, but those paths
fail closed with `command_not_available`; they are not supported commands.

The bare `heyfood` invocation is informational only. It prints runnable next
steps and never starts a TUI, browser, registration, onboarding, or network
request.

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
