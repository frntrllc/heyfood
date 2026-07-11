# Developing heyfood

The public heyfood repository is a standalone Python project. The proprietary
hello.food backend is not required to install the package, inspect help, develop
rendering, or run the unit and compatibility suites.

## Requirements

- Python 3.11, 3.12, or 3.13
- Git

## Set up a development environment

```bash
git clone https://github.com/frntrllc/heyfood.git
cd heyfood
python3.11 -m venv .venv
source .venv/bin/activate
python -m pip install --upgrade pip
python -m pip install -e ".[dev]"
python -m pytest -q
heyfood --help
```

When working from the private hello.food monorepo, run the same commands from
its `cli/` directory. Do not copy backend configuration or data into the public
repository.

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
src/heyfood_cli/   CLI implementation
  data/             Generated public dietary option contract
tests/             Unit and compatibility tests
pyproject.toml     Package metadata and build configuration
```

The committed compatibility fixtures under
`tests/fixtures/compat/0.1.0/` record the current help surface and representative
machine-output shapes. Intentional interface changes must update the relevant
fixtures and explain compatibility or migration behavior in the pull request.

The dietary option JSON is generated and must not be edited directly in the
standalone repository. See [the dietary catalog contract](docs/DIETARY_CATALOG.md)
for its public boundary, versioning, and private-monorepo synchronization flow.

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
- Use `--no-input` in automation tests and pair mutation approval with `--yes`;
  never rely on piped stdin satisfying an interactive prompt.
- Do not let banners, spinners, hints, warnings, or ANSI escapes enter JSON
  stdout.
- Keep safety conclusions conservative and explain uncertainty.
- Run the complete suite before requesting review.

See [CONTRIBUTING.md](CONTRIBUTING.md) for pull-request and licensing terms.
