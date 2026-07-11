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
- `get-menu` is the compatibility alias for `menu`.
- `reply TEXT` and `conversation resume TEXT` both continue the last locally
  remembered agent conversation.
- `chat --new` starts without the local conversation pointer; `conversation
  clear --yes` forgets that pointer without deleting server data.
- Onboarding preserves `--no-interactive` as the compatibility alias described
  in the public process contract; new automation should use `--no-input`.

## Discovering opaque ids

Use `heyfood members list` before passing `--member-id`. It lists synced member
profile ids returned by the service. Use `heyfood conversation list` to inspect
the one conversation id remembered in local CLI state, then `conversation
resume` or `conversation clear --yes`. The service does not currently expose a
conversation-history listing API, so the CLI does not imply that local state is
a complete history.
