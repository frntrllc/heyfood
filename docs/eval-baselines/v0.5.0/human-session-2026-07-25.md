# `v0.5.0` human TUI session — 2026-07-25

## Environment

- Public installed artifact: `heyfood 0.5.0`
- Platform: macOS Apple Silicon
- Launch path: `/opt/homebrew/bin/heyfood`
- Credential state: no Rust credential present in Keychain
- Production mutation: none

## Observations

Bare `heyfood` displayed a concise welcome and immediately began device
authorization with create-account intent. The terminal copy did not expose the
separate existing-account path provided by `heyfood login`. This is meaningful
first-run friction for a returning hello.food user and is recorded as
[issue #29](https://github.com/frntrllc/heyfood/issues/29).

Ctrl-C cancellation was immediate and trustworthy: the client stated that
registration was canceled and nothing was saved. A subsequent Keychain lookup
found no stored credential.

## Scores

| Dimension | Score | Result |
|---|---:|---|
| Initial orientation | 4/5 | Welcome and next action are concise |
| Existing-account discoverability | 2/5 | P2 finding; login path is hidden |
| Cancellation clarity | 5/5 | Explicitly non-mutating |
| Credential hygiene | 5/5 | No credential persisted after cancel |
| Authenticated conversation and Grocery experience | Not scored | No local credential; requires dedicated evaluation identity |

This is a bounded human observation, not the weekly full-session baseline. It
does not include or preserve the device code, email, token, household data,
dietary data, or any authenticated prompt.
