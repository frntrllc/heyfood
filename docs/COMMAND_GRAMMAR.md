# heyfood command grammar

heyfood keeps command input predictable while preserving the existing 0.1.0
surface for scripts. New aliases are additive; established commands are not
renamed solely for aesthetic consistency.

## Input rules

| Input | Grammar | Examples |
|---|---|---|
| A required free-form request | Positional words | `heyfood ask can I eat pad thai?`, `heyfood recipes search quick Thai dinner` |
| An optional filter | Named option | `heyfood search --query thai`, `heyfood recommend 1 --query vegan` |
| A resource selector | Positional id/ref/index | `heyfood menu 2`, `heyfood recipes save spoonacular:123` |
| Location or output controls | Named options | `--near`, `--lat/--lng`, `--json`, `--no-location` |
| A repeated structured value | Repeatable named option | `--allergy peanuts --allergy shellfish` |

Restaurant `search` deliberately keeps `--query` optional because a location-
only search is valid. Recipe search requires text, so its query remains
positional. `profile` and `daily` remain top-level compatibility commands;
moving them under new noun groups would add ceremony and break scripts without
improving discovery.

## Compatibility aliases

- `--raw` is the deprecated machine-output alias for `--json`.
- `register` creates or connects an account through the same OAuth application
  service as `login`; registration adds `--no-onboard` and canonical `--json`.
  `register --json` suppresses browser launch and all prompts. Use `--device
  --no-browser --json` for headless automation with one human approval.
- `login` performs an explicit sign-in for an already connected native account.
  It is also the only scope-upgrade path: refresh and session re-exchange never
  add authority. The replacement preserves every existing canonical scope and
  commits only after both channel and app-session grants are complete.
- `account delete` requires explicit destructive acknowledgement and browser
  identity confirmation. `--json` additionally requires `--yes`, never opens a
  browser, and emits a receipt only after the backend commit. Local credentials
  remain intact for denial, expiry, interruption, timeout, or malformed output.
- `get-menu` is the compatibility alias for `menu`.
- `reply TEXT` and `conversation resume TEXT` both continue the last locally
  remembered agent conversation.
- `chat --new` starts without the local conversation pointer; `conversation
  clear --yes` forgets that pointer without deleting server data.
- `--for NAME_OR_ID`, `--for me`, and `--for everyone` override household
  scope for an agent command. `household use` changes the persisted default;
  `/for` changes it inside chat and starts a fresh conversation.
- Onboarding preserves `--no-interactive` as the compatibility alias described
  in the public process contract; new automation should use `--no-input`.
- `--voice` on `onboard`, `ask`, and `log` speaks the request instead of typing
  it; the positional request text is optional when `--voice` is used, and
  positional text and `--voice` are mutually exclusive. Every mode shows the
  transcript for a review menu (accept / edit / record again / type instead /
  cancel) before submitting.
- `--voice` is interactive-only: combined with `--json`, `--raw`, `--no-input`,
  a non-TTY stdin/stderr, `CI`, or `TERM=dumb` it produces one stable
  noninteractive error and never opens a microphone or browser. Voice-only
  controls (`--voice-capture`, `--audio-device`) fail locally when given without
  `--voice`.
- `auto` never crosses from native transcription to browser speech recognition
  (a different, browser-vendor processor) without an explicit, default-no
  consent; an explicit `--voice-capture native` never opens a browser.
- `--voice-capture auto|native|browser|typed` (default `auto`) selects the
  capture mechanism; `--audio-device <id-or-name>` picks a microphone for native
  capture. Both are additive named options.
- `--voice-timeout` and `--no-browser` are browser-rung controls only: they tune
  the localhost browser capture and do not affect native or typed capture.

## Voice device discovery and preferences

`heyfood voice devices` lists input devices for native capture (index, name,
default marker). Pass the index or a name substring to `--audio-device`. Without
the `voice` extra installed, it prints an enable hint instead of a device list.

`heyfood voice status`, `heyfood voice set` (`--mode`, `--device`), and
`heyfood voice reset` inspect and manage persisted, reversible capture
preferences. An omitted mode stays distinct from an explicit `auto`, so a stored
preference can never silently cross processors or open a browser.

## Discovering opaque ids

Inside the Rust TUI, `/grocery` opens the live capability-gated active list and
`/health` opens the live provider-neutral integration/context view. `/profile`
reads consent and the synchronized dietary profile, while `/household` and
`/location` render account-bound local context without a network request. These
panels are read-only and cancellable. Voice and the mutating `/for` household
target switch remain absent from discovery until their complete typed workflows
are connected.

Use `heyfood members list` before passing `--member-id`. It lists synced member
profile ids returned by the service. Use `heyfood conversation list` to inspect
the one conversation id remembered in local CLI state, then `conversation
resume` or `conversation clear --yes`. The service does not currently expose a
conversation-history listing API, so the CLI does not imply that local state is
a complete history.

`heyfood household list` reconciles synced ids into the local roster. Use
`household label MEMBER_ID --name NAME --relationship RELATIONSHIP` when a
profile created on another device has no local display name, then use a unique
name or exact member id as the scope selector.
