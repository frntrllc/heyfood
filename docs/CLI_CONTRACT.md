# heyfood CLI contract

This document defines the public process interface for heyfood. Human rendering
may improve between compatible releases; the machine interface changes only
through an announced compatibility policy.

## Streams

### Standard output

In human mode, stdout contains the requested result. In `--json` mode, stdout
contains exactly one UTF-8 JSON value followed by one newline.

JSON stdout never contains:

- ANSI escape sequences;
- banners, spinners, or progress events;
- continuation hints or deprecation warnings;
- human diagnostics; or
- text before or after the JSON value.

### Standard error

stderr contains progress, loading status, deprecation warnings, and human error
diagnostics. Programs should not parse human stderr as a data format.

Global `--verbose` adds safe request lifecycle diagnostics to stderr only. It
does not change JSON stdout. Verbose fields are restricted to generated request
ids, method/path, selected context, status, elapsed time, and named refresh or
retry events; bodies, query text, authorization material, keys, profile data,
and phone numbers are prohibited.

## JSON mode

`--json` is the canonical machine-output flag for data-returning commands.
`--raw` is a deprecated compatibility alias that uses the same writer and
emits its deprecation warning to stderr.

Successful JSON generally preserves the documented service response for the
command. Local commands use an explicit object, for example:

```json
{"location": {"label": "Home", "latitude": 35.28, "longitude": -120.66}}
```

Machine-readable failures use this envelope and a nonzero exit code:

```json
{
  "ok": false,
  "error": {
    "type": "login_required",
    "message": "Run `heyfood login` first.",
    "hint": "Run `heyfood login` and retry."
  }
}
```

The `hint` field is optional. Consumers must tolerate additive fields.

Interactive `chat` does not have a JSON streaming protocol and rejects
`--json`. Use `ask --json` and `reply --json` for one-result interactions.
When the agent emits an out-of-band choice set, those commands add a `choices`
object containing `choices[]` and `allow_multiple` to the result document.
Confirmed local household writes are reported under additive `client_effects`.

`register --json` waits for one browser/device authorization decision and then
emits exactly one terminal result. It never opens a browser or starts dietary
onboarding:

```json
{
  "schema_version": 1,
  "authenticated": true,
  "account_outcome": null,
  "profile_status": "missing",
  "next_command": "heyfood onboard"
}
```

`profile_status` is `ready`, `missing`, or `unknown`. A service or contract
failure after authentication is `unknown`, never guessed as `missing`, and does
not remove the valid session. `account_outcome` is `null` because the OAuth grant
does not expose a trustworthy created/existing distinction; the CLI never
guesses it. Identity resolution remains authoritative in the browser/backend.
This exact shape is the shared first-run contract and `registrationResult` in
the v1 JSON schema.

Versioned safety, restaurant-fit, menu-evaluation, recommendation-ranking, and
recipe-compatibility core shapes are documented in
[`JSON_SCHEMAS.md`](JSON_SCHEMAS.md) and
`schemas/v1/heyfood-output.schema.json`. Safety-bearing fields use
`generally_safer`, `risky`, `avoid`, or `unable_to_evaluate`; the writer
normalizes legacy safety aliases without changing operational job statuses.

## Exit codes

| Code | Meaning |
|---:|---|
| `0` | The requested operation completed successfully. |
| `1` | Authentication, service, diagnostic, or incomplete runtime result. |
| `2` | Invalid invocation, missing local input, or local validation error. |

For example, unauthenticated `status` and an unhealthy `doctor` exit `1`.
Invalid paired options such as `--lat` without `--lng` exit `2`. Menu
acquisition that remains pending at the 30-second client ceiling returns the
pending JSON or human resume guidance and exits `1`.

## Local validation

heyfood validates request bounds before calling the service. The initial public
contract includes:

- latitude `-90..90` and longitude `-180..180`, supplied together;
- restaurant radius `0.1..50` miles;
- search, recommendation, recipe-search, and saved-recipe limits of `50`, `20`,
  `20`, and `100` respectively;
- agent queries up to 500 characters and food/restaurant/recipe queries up to
  200 characters where required by the service contract;
- recipe cuisine up to 80 characters, notes up to 1000 characters, and
  documented meal-type choices; and
- daily dates in ISO `YYYY-MM-DD` form.

Empty required text, non-finite numbers, out-of-range values, and half-specified
coordinate pairs fail locally with exit code `2`.

## Prompts and automation

`--no-input` is the explicit prompt-suppression contract for prompt-capable
commands. Non-TTY stdin is treated the same way even when the flag is omitted:
the command either has enough options to continue or fails with exit code `2`
and actionable guidance.

Mutating onboarding commands require explicit approval with `--yes` when input
is disabled. `--yes` approves consent and the final mutation only; it does not
disable unrelated guided questions. The legacy `--no-interactive` onboarding
flag remains a compatibility alias for disabling guided questions.

`conversation clear` follows the same automation rule: `--json`, `--no-input`,
and non-TTY use require `--yes`. It clears only the local resume pointer and
does not claim to delete server conversation data.

Household roster commands never prompt. `household list` refreshes synced
profile ids unless `--local-only` is passed; `current`, `use`, and `label` are
local configuration operations. Dietary contents are loaded just in time for
agent requests and are never persisted in the CLI roster. Child profiles are
the privacy-preserving exception: they stay in protected local storage and
never use server profile sync. The OS keyring is used when available, with the
documented owner-only `0600` file as the headless fallback. Failed adult sync
writes also stay in this protected store as a lossless repair outbox, continue
to scope agent turns, merge into later writes, and retry automatically on a
consented scoped turn. Confirmation previews are stricter: they are vault-only,
redacted from `config show`, and are not persisted between processes when no
vault is available. Account-scoped state is bound to the authenticated user and
cleared before saving credentials for a different user.

When synced-member discovery returns `403: Sync consent required`, `household
list` succeeds with the local roster and adds a `reconciliation` object with
`status: skipped`, `reason: profile_sync_consent_required`, and
`source: local_roster`. No other API error is downgraded.

`onboard --dry-run --no-input` performs no network call, credential/config
write, consent grant, or prompt. Interactive `chat` rejects `--no-input` and
non-TTY stdin; automation uses `ask` and `reply` instead.

Bare `heyfood` owns an interactive first-run state machine only when stdin,
stdout, and stderr are usable TTYs. A fresh local state recommends registration;
prior account state recommends sign-in. After authentication it strictly checks
profile readiness, offers the existing onboarding flow when the profile is
missing (typed input by default, with contextual voice or defer choices), and
enters chat after readiness or an intentional defer.
Non-TTY bare execution never performs network, credential, prompt, browser, or
profile actions and exits `0` after printing plain next steps.

## Compatibility and deprecation

- Additive JSON fields are compatible changes.
- Removing or renaming commands, options, JSON fields, error types, or exit-code
  meanings requires release notes and migration guidance.
- `--raw` remains an alias through at least the first public minor release and
  may be removed only in a versioned breaking release.
- Human spacing, colors, tables, and prose are not stable machine interfaces.
- Compatibility fixtures under `tests/fixtures/compat/` must be reviewed and
  updated with every intentional interface change.
