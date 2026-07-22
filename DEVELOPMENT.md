# Developing heyfood

The public heyfood repository is a native Rust workspace. The proprietary
hello.food backend is not required to build the executable, inspect help,
develop the TUI, or run the unit and contract suites.

## Requirements

- The Rust toolchain declared by `rust-toolchain.toml`
- Git

## Set up a development environment

```bash
git clone https://github.com/frntrllc/heyfood.git
cd heyfood
cargo build --release --locked --package heyfood-bin
cargo test --locked --workspace --all-targets
./target/release/heyfood --help
```

Do not copy backend configuration or data into the public repository.

## Source-of-truth and synchronization policy

The initial public repository is created once from a reviewed, committed
allowlist; it is not a subtree or history-preserving copy of the private
monorepo. Until that bootstrap commit is published, the private monorepo is the
staging source and public contributions remain closed.

After bootstrap, `frntrllc/heyfood` is canonical for CLI implementation,
tests, public schemas, and contributor documentation. CLI changes must begin
there and flow into the private monorepo through a reviewed, content-only sync.
Service-contract changes begin in the owning service repository and are exposed
to the CLI only through additive public contracts. Never merge monorepo history,
backend source, prompts, production configuration, or private test data into
the public repository.

## Project layout

```text
crates/heyfood-bin/             Native executable and composition root
crates/heyfood-cli/             Command grammar and output contracts
crates/heyfood-tui/             Interactive Ratatui application
crates/heyfood-agent-runtime/   Authenticated service transport and SSE
crates/heyfood-application/     Use cases and ports
crates/heyfood-core/            Validated domain types
crates/heyfood-platform/        Native persistence and OS integration
assets/                         Public, versioned runtime assets
```

The committed fixtures and schemas record reviewed wire, command, and migration
contracts. Intentional interface changes must update the relevant fixtures and
explain compatibility or migration behavior in the pull request.

See [the dietary catalog contract](docs/DIETARY_CATALOG.md) for the public asset
boundary, versioning, and private-monorepo synchronization flow.

## Service-backed development

Unit tests use fakes and temporary config paths. They must not require network
access, production credentials, or a checkout of the proprietary service.

Manual integration testing can target an authorized compatible service with:

```bash
export HEYFOOD_API_URL="https://api.example.test"
export HEYFOOD_AUTH_URL="https://auth.example.test/authorize"
export HEYFOOD_API_KEY="..."  # only when the target environment requires one
heyfood login
```

Never commit these values. Use a dedicated non-production account and synthetic
dietary data.

## Quality expectations

- Preserve stable command names and machine-output contracts unless a reviewed
  migration is included.
- Test stdout, stderr, exit codes, TTY/non-TTY behavior, and non-interactive
  paths for command changes.
- Keep interactive terminal tests isolated behind a PTY and verify restoration
  after normal exit, cancellation, signals, and panics.
- Do not let banners, spinners, hints, warnings, or ANSI escapes enter JSON
  stdout.
- Keep safety conclusions conservative and explain uncertainty.
- Run the complete suite before requesting review.

See [CONTRIBUTING.md](CONTRIBUTING.md) for pull-request and licensing terms.
