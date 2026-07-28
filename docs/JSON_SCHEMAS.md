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

## Agent-native contracts

The supported v0.6.0 release exposes its public installed contracts through
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
