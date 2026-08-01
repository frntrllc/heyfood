# Workflow selection

Select by surface first. The local and remote surfaces share no tool names, so
a tool that is absent is absent — not renamed.

## Local surface

### Discovery

Run `heyfood agent describe` first. It is offline, credential-free, and
version-matched to the executable.

Use these surfaces in order:

1. A listed `heyfood_*` MCP tool with an input schema matching the task.
2. An exact manifest command whose audience is `agent_safe`.
3. A truthful user handoff.

Never treat MCP absence as permission to invoke a different audience.

### Supported read intents

When present, prefer:

- `heyfood_get_manifest` for installed contract discovery;
- `heyfood_get_status` for service, authorization, and local readiness;
- `heyfood_get_capabilities` for server-advertised support;
- `heyfood_get_grocery_list` for the active household-aware list;
- `heyfood_get_grocery_exclusions` for never-buy exclusions; and
- `heyfood_list_menu_watches` for recurring watch summaries.

Do not guess that a service failure means an empty result.

Collection tools return at most 100 records. When `page.next_cursor` is
present, pass it back unchanged with an optional `limit` from 1 through 100.
Do not parse, edit, or fabricate cursors. If the server returns
`mcp_cursor_stale`, restart at the first page and do not combine pages from
different snapshots.

## Remote surface

### Discovery

There is no manifest and no binary. Your tool list is the contract. Do not run
`heyfood agent describe`, and do not report its absence as a fault.

Use these in order:

1. A listed hello.food tool whose input schema matches the task.
2. A truthful user handoff.

There is no command fallback on this surface. If no tool matches, stop.

### Supported read intents

When present, prefer:

- `lookup_restaurant` and `search_restaurants` to identify a restaurant;
- `get_menu_status` to check whether a menu has been captured;
- `evaluate_menu` to assess menu items against the user's dietary profile;
- `explain_item` for a single dish, with its reasons and conflicts;
- `recommend_items` for fitting choices, and `draft_order_message` to phrase an
  order request;
- `ask_dietary_question` for general dietary knowledge;
- `describe_dietary_graph`, `get_food_preferences`, and `get_meal_history` for
  profile-derived context; and
- `search_recipes`, `list_saved_recipes`, and `suggest_recipes` for recipes.

A hosted deployment may expose a subset. Absent tools are unavailable
capabilities, not tools to be reached another way.

### Not available remotely

**Grocery and Menu Watch have no remote tools and no corresponding
authorization scopes.** Do not describe them as temporarily unavailable, do not
retry, and do not offer to enable them. Tell the user they require the local
hey.food client.

### Cold menus

A restaurant may have no captured menu. Evaluation tools then return a typed
"menu not found" result. That is a real answer about coverage — report it as
such. Never present it as a safety judgement, and never infer that a dish is
acceptable because no menu was found.

## Human experience

Bare `heyfood` and `heyfood chat` are human terminal experiences. Tell the
user how to launch them when appropriate, but never control them. They exist
only where the local client is installed.

## Deferred capabilities

Health, default-build native voice, and public Windows distribution remain
deferred unless the exact installed manifest says otherwise. Do not infer
support from source files, hidden commands, plans, or marketing history. On the
remote surface, infer support only from the tool list you were given.
