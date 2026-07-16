# heyfood

Developer-first command-line access to personalized food and dietary guidance
from [hello.food](https://hello.food).

heyfood is the open-source CLI client. It provides terminal-native workflows
for dietary profiles, food checks, restaurant and menu discovery, recipes, meal
logging, and the hello.food conversational agent. Guidance is provided by the
hosted hello.food service and is not a substitute for professional medical
advice or emergency care.

## Installation

On macOS or Linux, run the hosted installer:

```bash
curl -fsSL https://hey.food/install.sh | bash
```

The public [installer source](install.sh) selects a supported Python, uses or
bootstraps an isolated [`pipx`](https://pipx.pypa.io/stable/installation/),
installs only `heyfood-cli` from PyPI, and verifies `heyfood --version`. It does
not use `sudo`, install Python or Homebrew, edit shell startup files, or start
authentication. If the command directory is not already on `PATH`, it prints
the exact export command. Python 3.11, 3.12, and 3.13 are supported.

To inspect pinned repository bytes before running them, replace `REVISION` with
a reviewed full commit SHA from this repository, then download and verify both
files from that immutable revision:

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

The SHA-256 file is meaningful only when it comes from a separately reviewed,
pinned repository revision. Fetching a script and checksum from the same
mutable endpoint does not protect against that endpoint being compromised.

To select an exact release or add operating-system credential-vault support:

```bash
curl -fsSL https://hey.food/install.sh | HEYFOOD_VERSION=0.2.0 bash
curl -fsSL https://hey.food/install.sh | HEYFOOD_WITH_KEYRING=1 bash
curl -fsSL https://hey.food/install.sh | HEYFOOD_WITH_VOICE=1 bash
```

Direct `pipx` installation remains fully supported:

```bash
pipx install heyfood-cli
heyfood --version
heyfood --help
```

With an existing `pipx`, install the optional keyring extra directly with:

```bash
pipx install 'heyfood-cli[keyring]'
```

To capture voice directly from your microphone (see [Voice input](#voice-input)),
install the optional voice extra:

```bash
pipx install 'heyfood-cli[voice]'
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

## Get started

Run the bare command after installation. In a terminal, it takes a new user
from account creation through optional dietary onboarding and into a useful
session. In a pipe or other non-interactive environment it only prints concise
next steps and never opens a browser.

```bash
heyfood
```

New users can also start explicitly:

```bash
heyfood register
```

Registration preflights the production capability contract before opening the
browser. Identity verification, current Terms and Privacy acceptance, account
resolution, and capability consent stay on `auth.hello.food`. If the first
dietary profile is missing, interactive registration offers the existing
guided onboarding immediately. Choose `type`, `voice`, or `later`; voice is an
optional path and never blocks typed onboarding. Use `--no-onboard` to defer it.

For a headless machine, use the one-decision short-code flow. JSON mode never
opens a browser or starts onboarding; progress and the approval URL use stderr,
and stdout receives exactly one JSON result after the decision:

```bash
heyfood register --device --no-browser --json
```

SMS registration is US-only. Email remains available without a phone. The auth
page advertises only identity methods that are genuinely enabled.

## Sign in to an existing account

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

Account deletion is a separate, deliberately destructive flow. It requires a
fresh CLI session with `account:delete`, explicit local acknowledgement, and a
second identity confirmation in the browser. Local credentials are removed
only after the service returns a validated post-commit receipt:

```bash
heyfood account delete
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
heyfood household list
heyfood household use everyone
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

## Households and conversational scope

heyfood can ask for you, one household member, or everyone. The local roster
mirrors the mobile app's local-first household model. Adult dietary graphs are
loaded from profile sync only for the active turn; child graphs remain local.

```bash
heyfood household list
heyfood household label MEMBER_ID --name Sarah --relationship spouse

heyfood household use Sarah
heyfood ask "What can she order here?"

heyfood ask --for everyone "What can we all eat?"
heyfood chat --for Sarah
```

`household list` discovers synced member profile ids. Because names and
relationships remain device-local by design, a member created in the mobile
app may first appear by id; use `household label` once on this machine. Child
members remain device-local and do not appear through sync. Inside
chat, `/household` shows the roster and `/for Sarah`, `/for everyone`, or `/for
me` switches scope and starts a fresh conversation. Numbered agent choices can
be answered with `2` or, for multi-select questions, `1, 3`.

On an account without profile-sync consent, `household list` still returns the
local roster and marks synced-member reconciliation as skipped in JSON output.
Other authentication and service errors remain failures rather than being
silently hidden.

As in the mobile app, child dietary profiles stay on this machine and never
enter profile sync. Other members' dietary graphs are loaded from profile sync
only for the active agent turn. If an adult profile write fails, heyfood keeps
the confirmed change in a protected local outbox, includes its full safety
context in later turns, and automatically retries it on a consented scoped
agent turn. A later mutation merges with pending fields instead of replacing
them, so subsequent safety checks do not silently lose an allergy write.

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

## Voice input

Speak instead of type. `--voice` is available on `onboard`, `ask`, and `log`:

```bash
heyfood onboard --voice
heyfood ask --voice
heyfood log --voice
```

The default capture mode is `auto`, which prefers native capture and never
changes speech processors without asking you first:

1. **Native microphone** (needs the `voice` extra) — records into memory and
   uploads the audio once over HTTPS to hello.food's authenticated transcription
   endpoint. The audio is processed by hello.food and its configured
   transcription provider, then discarded; it is never written to disk. The
   upload happens **before** you review the transcript — reviewing gates whether
   the resulting *text* is submitted to your profile, meal history, or the agent,
   not whether the audio is uploaded.
2. **Browser speech recognition** — a localhost page that uses your browser's
   built-in speech engine. This is a **different processor**: your browser vendor
   (a third party) receives the audio, not hello.food. In `auto` mode the CLI
   never crosses to it silently — if native capture is unavailable or fails, it
   asks first, defaulting to *no*, with a one-line disclosure. Declining goes
   straight to typed input.
3. **Typed input** — always works.

Before recording, native capture verifies your login carries the
`audio:transcribe` permission; an older session is asked to re-run
`heyfood login` **before** any audio is recorded. If your microphone's native
rate is above the supported range, the CLI negotiates a supported rate rather
than producing an upload the server would reject.

After capture, the transcript is shown for review: **accept**, **edit**, **record
again**, **type instead**, or **cancel** (nothing is submitted). A recording that
dropped audio can't be accepted as-is for a saved profile or meal — edit it,
record again, or type.

Choose a mode explicitly with `--voice-capture native|browser|typed`. An explicit
`native` never opens a browser: if native can't run, it falls back to typed input
(or asks you to re-login for a missing permission). `--voice` is interactive-only
— combined with `--json`, `--raw`, `--no-input`, a non-TTY, CI, or `TERM=dumb` it
prints one stable error and never opens a microphone or browser. Positional text
and `--voice` are mutually exclusive.

List and select microphones, and manage preferences:

```bash
heyfood voice devices              # list input devices (also `--json`)
heyfood ask --voice --audio-device 1
heyfood voice status               # show persisted capture preferences
heyfood voice set --mode native    # persist a preference (an omitted mode
                                   # stays distinct from an explicit `auto`)
heyfood voice reset                # clear persisted preferences
```

`--voice-timeout` and `--no-browser` apply to the browser rung only.

### Platform notes

- **Linux:** the `sounddevice` wheel needs the system PortAudio library. Install
  it with your package manager, e.g. `sudo apt-get install libportaudio2`.
  Without it, native capture is skipped; `auto` then asks (default no) before
  using browser speech recognition, otherwise types.
- **WSL:** prefer a native Windows install of the CLI for microphone access;
  inside WSL, native capture generally cannot reach the microphone, so `auto`
  asks before browser capture or falls back to typed input.
- **SSH / headless:** the browser capture server binds on the remote host and
  can't reach your local microphone, so over SSH voice falls back to typed input
  with an explanation rather than opening a dead localhost URL.

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
`keyring` extra to keep access tokens, refresh tokens, local API keys, household
identity, local-only child dietary profiles, and pending profile-sync repairs
in the operating-system credential vault; the JSON file then contains only
non-secret state and a `credential_store: "keyring"` marker.

Confirmation previews can contain names, birth dates, and dietary data. They
are persisted only in the credential vault and are omitted from both the JSON
file and `heyfood config show`; without a usable vault, heyfood does not persist
the preview between processes. Local household state is bound to the
authenticated account and is cleared fail-closed when a different account logs
in (including legacy unbound state on the first upgraded login).

On headless systems without a usable keyring, credentials and protected
household state remain in the `0600` file. This protects them from other local
users under normal permissions, but not from malware or another process running
as the same OS account. Set
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
pipx uninstall heyfood-cli
```

The hosted installer prints the exact `pipx` command that manages its
installation. When it had to create its own isolated bootstrap, that command
uses the Python under
`${XDG_DATA_HOME:-$HOME/.local/share}/heyfood/installer/pipx/`. If logout cannot
reach the service, local credentials are still removed but server-side sessions
may persist until expiry. Use `heyfood logout --json` to inspect per-step
teardown results without exposing token values.

## License and project boundary

Copyright 2026 FRNTR, LLC.

The heyfood CLI distribution is licensed under the
[Apache License 2.0](LICENSE). The license applies to this client and explicitly
published public assets. It does not license the proprietary hello.food
backend, hosted service, intelligence, models, prompts, data, evaluation rules,
or infrastructure.
