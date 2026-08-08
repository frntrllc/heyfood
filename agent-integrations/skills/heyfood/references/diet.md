# Diet guidance

Diet guidance is a capability-discovered, read-only local surface. It explains
the hello.food corpus for a named eating pattern; it does not change a profile,
assess a particular food, or grant mutation authority.

## Availability

Use the Diet tools only when all of these agree:

1. the accepted manifest schema advertises the Diet guidance capability as
   active;
2. live capability discovery reports exactly `diet:v1`; and
3. the matching `heyfood_list_diets` or `heyfood_get_diet` tool is present.

Missing and unknown capability versions fail closed. Do not infer Diet support
from a binary version, documentation, a profile value, or a similarly named
remote tool. The hosted MCP surface does not expose this Diet catalog.

## Catalog and detail

Call `heyfood_list_diets` first when the person has not supplied an exact Diet
ID. Preserve each returned ID exactly: IDs are case-sensitive and must not be
trimmed, normalized, localized, or guessed from the display label.

Call `heyfood_get_diet` with one exact catalog ID. A successful
`diet_not_covered` card is a coverage answer, not a transport failure. Report
that hello.food has no authored guidance for the diet; never invent or import
guidance from general model knowledge.

For covered cards:

- preserve the service's evidence grade;
- keep authored section order when presenting the complete card;
- always include the safety section;
- preserve contraindicated conditions and citations; and
- describe the content as advisory guidance, not medical clearance.

## Safety precedence

Diet guidance and optional Diet alignment annotations are subordinate to food
safety. They must never change or obscure a food's safety status, badge, color,
order, filter membership, per-member explanation, substitutions, or label
warning.

An item can be aligned with a diet and still be `avoid`; an item can be
off-diet and still be generally safer. Present both facts without reconciling
one into the other. Never state that an aligned item is safe.

## No mutation surface

There is no agent Diet set/clear tool. Do not use shell, stdin, the TUI, profile
sync, or a generic hosted call as a substitute. If a person asks to change a
declared diet, hand off to a supported human workflow when the installed
manifest advertises one; otherwise state that the current agent surface is
read-only.
