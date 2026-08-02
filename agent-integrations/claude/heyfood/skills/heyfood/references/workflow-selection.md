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

Your tool list is authoritative — this inventory describes a release, not a
ceiling. A newer client may expose `heyfood_*` tools not named here; read their
schemas and use them. Never withhold a tool the client advertises because this
list predates it, and never assume one it does not.

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

### Tool inventory

- `lookup_restaurant`, `search_restaurants` — identify a restaurant
- `get_menu_status` — polls a menu-fetch job. It requires a `job_id` issued by
  a fetch you started, so on a read-only deployment that never starts one there
  is nothing to poll. Do not call it to test whether a menu exists; that is not
  what it answers.
- `evaluate_menu`, `explain_item`, `recommend_items` — dietary assessment
- `draft_order_message` — phrase an order request
- `search_recipes`, `list_saved_recipes`, `suggest_recipes` — recipes
- `ask_dietary_question` — general dietary knowledge
- `describe_dietary_graph`, `get_food_preferences`, `get_meal_history` —
  profile-derived context

A hosted deployment may expose a subset. Absent tools are unavailable
capabilities, not tools to be reached another way.

### Restaurants and menus

**Resolve the restaurant before assessing anything.** Most assessment tools key
on `restaurant_id`, which you do not have from a user's words. Get it first:

- `lookup_restaurant(name)` when the user named a specific place.
- `search_restaurants(query, location_query, radius_miles, limit)` when they
  described one, or asked what is nearby.

If resolution is ambiguous, ask. Never assess a guess — two branches of a chain
can carry different menus, and a confident answer about the wrong location is
worse than a question.

**One asymmetry to know.** `explain_item(item_name, restaurant_name)` takes
plain names, while `evaluate_menu`, `recommend_items`, and
`draft_order_message` take `restaurant_id`. So a single-dish question can be
answered without resolution, but anything menu-wide cannot.

**Then choose by the shape of the question:**

| The user asks | Use |
|---|---|
| "Can I eat X here?" | `explain_item` — one dish, with reasons and conflicts |
| "What's safe on this menu?" | `evaluate_menu(restaurant_id, item_names)` |
| "What should I order?" | `recommend_items(restaurant_id, query, …)` |
| "Ask them to leave out the X" | `draft_order_message` |

`evaluate_menu` takes explicit `item_names`. It assesses what you give it — it
does not fetch a menu for you to browse. If the user has not named items, either
ask which dishes they are considering, or use `recommend_items` instead.

**Preserve the verdict.** Carry the service's own safety wording, reasons,
conflicts, allergen detail, alternatives, and any freshness or provenance
markers through to the user unchanged. Do not re-rank, re-summarize, soften, or
convert a status into your own phrasing. Never state that a food is "safe".

**Cold menus** are covered below. Treat them as coverage, not safety.

### Households — read this before answering for anyone

hello.food is built for people who decide what someone *else* eats. Getting the
scope wrong does not produce a vague answer; it produces a confident answer
about the wrong person.

**The default is household-wide, not you.** On an account with synced members,
a call that passes no scope is evaluated for the WHOLE household. Do not assume
an unscoped call means the account owner.

`household_scope` accepts exactly three things:

| Value | Means |
|---|---|
| `"_self"` | the account owner alone |
| `"everyone"` | all members |
| a member id | that member alone |

**`member_id="_self"` does NOT mean "just me".** It is the argument's default
and carries no scope at all, so it falls through to default resolution — which
on a member-having account is household-wide. To mean the owner alone you must
send `household_scope="_self"` explicitly. `member_id` is a legacy alias kept
for deployed clients: an explicit `household_scope` always wins, and any
`member_id` other than `_self` means that member.

**Never infer scope from phrasing.** "Can my daughter eat this" does not
license guessing a member. Resolve the member, or ask. The service honours the
scope you send authoritatively and never reads intent from wording — so an
unscoped call for a named person is a wrong answer, not an approximate one.

**Read the aggregate correctly.** In `evaluate_menu`, each item is assessed
once per member in scope. **The headline status on an item is the household
AGGREGATE — worst status wins** — and the additive `member_annotations` say
WHO. Reporting the headline alone tells a caregiver "avoid" without saying it
is avoid *for one member*, or lets them read a household verdict as being about
themselves. Always carry the per-member annotations through with the headline.

Flags on an item are informational; these tools never drop an item from the
result. Absence of a warning is not a clearance.

**Scope rejections are answers, not retry conditions:**

- **422** — the scope was malformed, or needs an account capability this one
  does not have.
- **404** — a well-formed member id that is not a synced member of THIS
  account.

Neither is ever degraded into a different evaluation set, and you must not
degrade it either. Do not retry unscoped, do not substitute `_self`, and do not
fall back to the owner. Report which member was not found and stop.

### Recipes

Three tools, three different jobs — picking the wrong one gives the user
something that looks like an answer and is not:

- `search_recipes(query, cuisine, meal_type, max_ready_time, limit)` — find
  existing recipes matching a request.
- `suggest_recipes(recipe_title, ingredients, constraints, servings)` — adapt a
  specific recipe the user already has in mind, for their constraints. Pass
  their actual constraints; do not restate them into your own words.
- `list_saved_recipes(limit)` — what the user already saved. Use it before
  suggesting something new when they refer to "my" recipes.

Recipe results carry dietary reasoning for the same reason menu results do.
Preserve it. A substitution offered by the service is part of the answer, not a
detail to compress away.

**Saving is a mutation** and is not available here. If a user asks to save a
recipe, say so plainly rather than implying it was kept.

### Households on the local surface — human-only

The client has household onboarding, encrypted local household profiles, and
scope switching. **None of it has an agent surface.** Verified against the
0.7.0 manifest: no household capability, no household or member command, and
the local MCP tool list is unchanged — the same six read tools.

So on the local surface you cannot read the roster, cannot set or switch scope,
and cannot evaluate for a member. Household management lives in the human TUI
and is reached by the user running `heyfood`. Do not drive it, do not offer to,
and do not present a Grocery read as household-scoped beyond whatever scope the
result itself reports.

If a user asks for household work and you only have the local surface, say it
is done in the hey.food terminal app by them. If you also have the hosted
surface, household-scoped evaluation is available there — see above.

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
