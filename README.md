# heyfood

Developer-first command-line access to personalized food and dietary guidance
from [hello.food](https://hello.food).

heyfood is the open-source CLI client. It provides terminal-native workflows
for dietary profiles, food checks, restaurant and menu discovery, recipes, meal
logging, and the hello.food conversational agent. Guidance is provided by the
hosted hello.food service and is not a substitute for professional medical
advice or emergency care.

## Installation

Install the isolated command with
[`pipx`](https://pipx.pypa.io/stable/installation/):

```bash
pipx install heyfood-cli
heyfood --version
heyfood --help
```

Python 3.11, 3.12, and 3.13 are supported. To use the operating-system
credential vault instead of the secure file fallback, install the optional
keyring extra:

```bash
pipx install 'heyfood-cli[keyring]'
```

Contributors can install from a reviewed source checkout:

```bash
git clone https://github.com/frntrllc/heyfood.git
cd heyfood
python3.11 -m venv .venv
source .venv/bin/activate
python -m pip install --upgrade pip
python -m pip install -e .
# Optional: store tokens in the operating-system credential vault
python -m pip install -e '.[keyring]'
heyfood --version
heyfood --help
```

## Authenticate

```bash
heyfood login
heyfood status
heyfood doctor
```

Login opens a browser for explicit hello.food consent and stores the resulting
CLI credentials securely. On an SSH/headless machine, use the short-code flow:

```bash
heyfood login --device --no-browser
```

Open the printed URL on any browser, confirm that the displayed code matches
the terminal, review the same capability list, and approve. The remote machine
never needs a reachable loopback callback.

## Common workflows

```bash
heyfood ask "What can I order at this restaurant?"
heyfood reply "Log the first option"
heyfood chat

heyfood item "pad thai" --restaurant "Pismo's"
heyfood search --near "Fresno, CA" --query "thai"
heyfood menu 1
heyfood recommend 1 --query "low-FODMAP dinner"

heyfood recipes search "Mediterranean dinner" --max-ready-time 35
heyfood recipes save 1
heyfood recipes saved

heyfood onboard
heyfood profile
heyfood members list
heyfood daily today

heyfood conversation list
heyfood conversation resume "What about the second option?"
```

Agent and direct commands render confirmations, safety verdicts,
restaurant/menu summaries, recipe ideas, and meal nutrition as terminal-native
output. The product uses “generally safer,” “risky,” “avoid,” and “unable to
evaluate” rather than presenting food as absolutely safe.

See the [command grammar](docs/COMMAND_GRAMMAR.md) for positional inputs,
selectors, compatibility aliases, and the rationale for optional search flags.

`recommend` shows a composite **Match** rank and ranking confidence, not a
safety verdict. Each row includes an `heyfood item ...` command for the separate
safety evaluation. The ranking and canonical safety JSON contracts are defined
in the [versioned schema guide](docs/JSON_SCHEMAS.md).

## Location

Restaurant search accepts coordinates, a one-off place name, or a saved default:

```bash
heyfood location set "San Luis Obispo, CA"
heyfood location set --lat 35.28 --lng -120.66 --label Home
heyfood location show

heyfood search --query "thai"
heyfood search --near "Fresno, CA" --query "thai"
heyfood search --lat 36.74 --lng -119.79 --query "thai"

heyfood location clear
```

Explicit coordinates take precedence over `--near`, which takes precedence
over the saved location. Place-name lookup requires a service deployment that
supports the geocoding channel tool.

Search and recipe results are remembered locally so later commands can use a
numbered selector such as `heyfood menu 1` or `heyfood recipes save 2`.

Saved location is also supplied to `ask`, `reply`, and `chat` when no explicit
location is provided. Override it with `--lat/--lng` or `--near`, or suppress
location for a request with `--no-location`.

## Dietary graph onboarding

Run `heyfood onboard` for the guided questionnaire, or start with natural
language and review the extracted values:

```bash
heyfood onboard "I'm low-FODMAP, avoid onion and garlic, and prefer Thai food"
```

In an interactive terminal, heyfood shows the extracted sections first, lets
you reject them in favor of the full questionnaire, and asks only for sections
the text did not answer. Explicit statements such as “no food allergies” count
as answered and are not prompted again.

Repeatable automation can pass fields directly:

```bash
heyfood onboard \
  --diet low-fodmap \
  --condition IBS \
  --allergy peanuts \
  --avoid onion \
  --avoid garlic \
  --cuisine Thai \
  --activity moderate \
  --yes \
  --no-input
```

Use `heyfood onboard --list-options` to inspect accepted labels and identifiers,
`--dry-run` to preview the profile payload, and `heyfood profile` to inspect the
synced graph. `--no-input` guarantees the command will never prompt; mutations
must combine it with `--yes`. Passing `none` (or an explicit empty repeatable
field from an API caller) clears only that source category and its derived
values; unrelated diet, allergy, condition, avoid, cuisine, and activity data
is preserved. `--replace` discards the complete existing graph before applying
the supplied fields.

## Machine output

Use `--json` for automation. Data commands emit exactly one ANSI-free JSON
value to stdout; progress, hints, and deprecation warnings use stderr. The
existing `--raw` flag is a deprecated compatibility alias for the same writer.

See the [CLI process contract](docs/CLI_CONTRACT.md) for JSON errors, exit codes,
prompt behavior, and compatibility policy.

Add global `--verbose` before a command to print safe request diagnostics to
stderr while leaving result/JSON stdout unchanged:

```bash
heyfood --verbose doctor --json
heyfood --verbose ask "Can I eat pad thai?" --json
```

Diagnostics include a generated request id, method/path, selected context,
status, timing, and auth refresh/retry event names. They never include request
bodies, query text, tokens, API keys, dietary profile contents, or phone
numbers.

## Shell completion

Install completions once for the current shell:

```bash
heyfood --install-completion  # auto-detects zsh, bash, or fish
```

To review or manage the script yourself, use `heyfood --show-completion`.
Restart the shell after automatic installation. In dotfile-managed setups,
capture the generated script instead of letting the CLI edit a shell file.

## Configuration

Configuration is stored at `~/.config/heyfood/config.json` by default, or under
`$XDG_CONFIG_HOME/heyfood/config.json` when `XDG_CONFIG_HOME` is set. The client
keeps the directory owner-only and the file mode at `0600`. Install the optional
`keyring` extra to keep access tokens, refresh tokens, and local API keys in the
operating-system credential vault; the JSON file then contains only non-secret
state and a `credential_store: "keyring"` marker.

On headless systems without a usable keyring, credentials remain in the `0600`
file. This protects them from other local users under normal permissions, but
not from malware or another process running as the same OS account. Set
`HEYFOOD_CREDENTIAL_STORE=file` to choose that fallback explicitly, or
`HEYFOOD_CREDENTIAL_STORE=keyring` to fail instead of falling back when the
vault is unavailable. Never copy either credential store between machines.

Authorized development environments can override service endpoints without
editing the config file:

```bash
export HEYFOOD_API_URL="https://api.example.test"
export HEYFOOD_AUTH_URL="https://auth.example.test/authorize"
export HEYFOOD_API_KEY="..."
```

Named contexts make production, local, and custom environments explicit:

```bash
heyfood context list
heyfood context use local
heyfood context set staging \
  --api-url https://api.staging.example \
  --auth-url https://auth.staging.example/authorize \
  --use
heyfood context show
```

Precedence is command flags, then `HEYFOOD_API_URL`/`HEYFOOD_AUTH_URL`, then
the selected context. `heyfood login --local` remains a convenient one-command
override. Inspect configuration without revealing credentials:

```bash
heyfood config path
heyfood config show --json
heyfood config validate
```

`config show` redacts all token and API-key fields. If validation reports
malformed JSON, move the named file aside and rerun `heyfood login`; read-only
configuration and doctor commands do not mint a device id or create config.

## Terminal branding and accessibility

The full hey.food banner appears only on compatible interactive terminals while
a genuinely slow operation is starting, and at most once per process. It is
never written to JSON/result stdout, never adds a delay, and is suppressed for
CI, redirected output, `TERM=dumb`, and non-interactive input. Terminals narrower
than 44 columns or unable to encode the block glyphs use a compact `hey.food`
welcome instead.

Use either control to disable decorative branding for screen readers or personal
preference:

```bash
heyfood --no-banner
export HEYFOOD_NO_BANNER=1
```

`NO_COLOR=1` keeps supported banner geometry but removes color.

Never commit the config file, tokens, or API keys. Public issues and fixtures
must use synthetic dietary and account data.

## Development and support

- [Development setup](DEVELOPMENT.md)
- [Contributing](CONTRIBUTING.md)
- [Changelog](CHANGELOG.md)
- [Release process](RELEASING.md)
- [CLI process contract](docs/CLI_CONTRACT.md)
- [Command grammar](docs/COMMAND_GRAMMAR.md)
- [JSON schemas](docs/JSON_SCHEMAS.md)
- [Dietary catalog contract](docs/DIETARY_CATALOG.md)
- [Support](SUPPORT.md)
- [Security policy](SECURITY.md)
- [Code of Conduct](CODE_OF_CONDUCT.md)

## Uninstall

Revoke the local session before removing the package:

```bash
heyfood logout
python -m pip uninstall heyfood-cli
```

For a future `pipx` installation, use `pipx uninstall heyfood-cli` after logout.
If logout cannot reach the service, local credentials are still removed but
server-side sessions may persist until expiry. Use `heyfood logout --json` to
inspect per-step teardown results without exposing token values.

## License and project boundary

Copyright 2026 FRNTR, LLC.

The heyfood CLI distribution is licensed under the
[Apache License 2.0](LICENSE). The license applies to this client and explicitly
published public assets. It does not license the proprietary hello.food
backend, hosted service, intelligence, models, prompts, data, evaluation rules,
or infrastructure.
