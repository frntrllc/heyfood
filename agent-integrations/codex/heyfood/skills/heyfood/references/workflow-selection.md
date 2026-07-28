# Workflow selection

## Discovery

Run `heyfood agent describe` first. It is offline, credential-free, and
version-matched to the executable.

Use these surfaces in order:

1. A listed `heyfood_*` MCP tool with an input schema matching the task.
2. An exact manifest command whose audience is `agent_safe`.
3. A truthful user handoff.

Never treat MCP absence as permission to invoke a different audience.

## Supported read intents

When present, prefer:

- `heyfood_get_manifest` for installed contract discovery;
- `heyfood_get_status` for service, authorization, and local readiness;
- `heyfood_get_capabilities` for server-advertised support;
- `heyfood_get_grocery_list` for the active household-aware list;
- `heyfood_get_grocery_exclusions` for never-buy exclusions; and
- `heyfood_list_menu_watches` for recurring watch summaries.

Do not guess that a service failure means an empty result.

## Human experience

Bare `heyfood` and `heyfood chat` are human terminal experiences. Tell the
user how to launch them when appropriate, but never control them.

## Deferred capabilities

Health, default-build native voice, and public Windows distribution remain
deferred unless the exact installed manifest says otherwise. Do not infer
support from source files, hidden commands, plans, or marketing history.
