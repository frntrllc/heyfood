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

`onboard --dry-run --no-input` performs no network call, credential/config
write, consent grant, or prompt. Interactive `chat` rejects `--no-input` and
non-TTY stdin; automation uses `ask` and `reply` instead.

## Compatibility and deprecation

- Additive JSON fields are compatible changes.
- Removing or renaming commands, options, JSON fields, error types, or exit-code
  meanings requires release notes and migration guidance.
- `--raw` remains an alias through at least the first public minor release and
  may be removed only in a versioned breaking release.
- Human spacing, colors, tables, and prose are not stable machine interfaces.
- Compatibility fixtures under `tests/fixtures/compat/` must be reviewed and
  updated with every intentional interface change.
