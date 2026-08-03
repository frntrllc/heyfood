# heyfood JSON schemas

The public machine-output schema is versioned at
`schemas/v1/heyfood-output.schema.json` using JSON Schema draft 2020-12. It
defines the stable core of six developer-facing result families while allowing
additive service fields:

| Commands/results | Schema definition |
|---|---|
| `item --json`, agent `safety_verdict` | `safetyVerdict` |
| `search --json` restaurant rows | `restaurantFit` |
| `menu --json`, agent menu evaluations | `menuEvaluation` |
| `recommend --json` | `recommendationRanking` |
| `recipes search --json` | `recipeCompatibility` |
| `register --json` | `registrationResult` |

The schema version is carried by the repository path and
`x-heyfood-schema-version`. heyfood intentionally does not wrap service
responses in a new envelope merely to repeat that number: existing consumers
keep the documented top-level response shape and pin the schema file for their
CLI major/minor compatibility range. The cross-client first-run registration
contract is the exception: its canonical object includes `schema_version: 1`.

Additive optional fields remain compatible within v1. Removing a field,
changing its meaning/type, or changing an enum requires a new schema version,
release notes, compatibility fixtures, and migration guidance.

## Safety vocabulary

Safety-bearing JSON uses exactly:

- `generally_safer` — a conservative relative conclusion, never a guarantee;
- `risky` — material concern or verification is required;
- `avoid` — the item conflicts with the evaluated dietary context; and
- `unable_to_evaluate` — evidence is insufficient for a conclusion.

The machine writer normalizes legacy `safe`/`safer`, `caution`, and `unsafe`
values at recognized safety fields. It does not rewrite operational statuses
such as menu acquisition `ready`, `failed`, or `timed_out`.

## Ranking is not a verdict

`recommendations[].score` is a 0–1 composite match/relevance rank. The service
may combine dietary compatibility, preference affinity, interaction history,
price fit, and menu freshness. It is neither a probability nor a safety status.
`confidence` describes confidence in the ranking. Run the emitted `heyfood item
...` command for a safety evaluation with the canonical status vocabulary.

Recipe `dietary_match_hint` is likewise compatibility ranking unless the result
contains a separate explicit safety assessment.

## Household evaluation contract

The v0.8.0 source tree freezes the reviewed hello.food household-evaluation
contract at
`fixtures/contracts/household-backend/v1/household-evaluation-contract.json`
and its founding Maya menu scenario beneath the adjacent `fixtures/` directory.
`provenance.json` records the exact hellofood source commit and tree, merge
ancestry, per-file SHA-256 digests, aggregate digest, and separately qualified
deployment evidence. Provider SDK types and secrets are not imported.

The additive contract supplies `household` snapshot identity and one named
`items[].member_annotations` entry per resolved member. Strict client parsing
recognizes only `generally_safer`, `risky`, `unable_to_evaluate`, and `avoid`;
unknown statuses or annotation dispositions and missing display labels fail
closed in human output. Human TUI/CLI presentation omits stable IDs, context
hashes, producer/rules versions, tool names, and raw JSON. It distinguishes an
informational `flag` from an allergen-driven `excluded` annotation and never
promotes `unable_to_evaluate` into an invented `avoid` verdict.

Machine output has a different obligation: `ask --json` and `reply --json`
emit the complete terminal agent result document unchanged in shape, including
all additive household/member annotations and unknown additive fields. This
JSON parity is intentional; the privacy-safe human projection must not be used
as a machine-data replacement. Single-member responses retain every
pre-existing result value, while the household fields remain purely additive.

## Agent-native contracts

The supported v0.8.0 release exposes its public installed contracts through
`heyfood agent schema --list`. The public allowlist contains:

- `schemas/v1/heyfood-agent-manifest.schema.json` freezes network-free
  discovery, supported automation surfaces, interactivity/browser behavior,
  required-nullable authorization/reconciliation metadata, authority/retry
  invariants, and resource limits.
- `schemas/v1/agent-proposal-presentation.schema.json` is the complete
  additional-properties-forbidden allowlist that may be shown to a model or
  human during a future agent proposal flow.
- `schemas/v1/heyfood-agent-schema-index.schema.json` freezes the name,
  identifier, digest, and size inventory for public schemas.
- `schemas/v1/heyfood-agent-doctor.schema.json` freezes privacy-safe offline
  diagnostics.
- `schemas/v1/heyfood-agent-setup-plan.schema.json` freezes the opt-in
  dry-run/apply/uninstall plan, exact binary and package identities, qualified
  host observations, conflicts, and typed user handoffs.
- `schemas/v1/heyfood-output.schema.json` remains the existing public result
  family contract.

`schemas/v1/agent-approval-protocol.schema.json` remains an internal
backend/implementation freeze and is intentionally absent from installed
schema discovery because it describes commit authority. The golden manifest
fixture is `fixtures/agent/manifest-v1-golden.json`. Self-description does not
by itself claim MCP support; Agent Skill setup is a separately versioned,
opt-in, receipt-bound surface.

The v0.8.0 agent boundary makes manifest and doctor schema v3 the default.
Explicit v1 and v2 remain frozen compatibility views, preserving their exact
fields and omitting the v3 household agent surface.

The explicit `--schema-version 2` discovery option uses
`schemas/v2/heyfood-agent-manifest.schema.json` and
`schemas/v2/heyfood-agent-doctor.schema.json`, named `manifest-v2` and
`doctor-v2`. The v2 manifest adds the exact top-level
`native_state_compatibility` declaration for the managed installer and release
verifier; a consumer receives that frozen view only by requesting v2.

Schema v3 adds the compatibility bootstrap, structured eight-tool MCP
inventory, and the two active household read commands/tools. Public discovery
also embeds the closed compatibility, household-context input,
household-member input, and household-read result schemas. Context profile
reads are rejected; minimized profile disclosure requires an exact additional
member reference.

## Agent-aware household schemas

The following closed schemas define the v0.8.0 read boundary and the separately
deferred mutation boundary:

- `schemas/v3/heyfood-agent-manifest.schema.json`;
- `schemas/v1/heyfood-agent-compatibility.schema.json`;
- `schemas/v1/agent-household-context-input.schema.json`;
- `schemas/v1/agent-household-member-input.schema.json`;
- `schemas/v1/agent-household-read.schema.json`;
- `schemas/v1/agent-household-action.schema.json`;
- `schemas/v1/agent-household-proposal-presentation.schema.json`;
- `schemas/v1/agent-household-outcome.schema.json`;
- `schemas/v1/household-agent-disclosure.schema.json`;
- `schemas/v1/local-household-approval-protocol.schema.json`; and
- `schemas/v1/agent-household-native-state.schema.json`.

The v3 schema is a structural successor, not an additive v2 edit. Explicit
v1/v2 views remain frozen and omit household agent claims. Proposal and
outcome schemas contain lookup/status evidence only and forbid commit
authority; they remain deferred and are not active MCP tool contracts. The
local approval protocol is distinct from the existing hosted
`agent-approval-protocol` schema. Fixtures and binding digests are under
`fixtures/agent/household-phase0/`; validation runs in the
`heyfood-agent-contract` test suite.
